# agit web UI

Clean, GitHub-like web UI for agit repositories: code browsing, server-side
blame, history, branches, compare view, pull requests with structured reviews,
live agent activity and notifications, the lease board, the knowledge base
(with staleness, drafts and relevance scores), repository search, signing keys
and signature badges, scoped access tokens, and settings (policy, keys,
webhooks, knowledge rules). It talks **only** to the public HTTP API in
[`docs/API.md`](../docs/API.md); there are no private endpoints. The one
feature the API does not offer server-side (code search) is computed in the
browser from public endpoints; client-side blame survives only as a fallback.

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
header's settings popover (⚙), where you also sign in with a bearer token and
set the acting agent id. The server can also serve the built UI itself:
`agit-server --web-dir web/dist` (SPA fallback included), which is how the
Playwright pass below runs it.

### Auth

When the server is started with a root token every request needs
`Authorization: Bearer …`. The UI calls `GET /auth/whoami` at startup, after
every settings change and after each mode switch, and shows the principal in
the header: avatar + agent id (or `root` / token id) + an access badge
(`root`, `admin`, `write`, `read`, `none`); hovering lists the scopes. Without
a valid token the chip reads **Sign in**. Any `401`/`403` the HTTP layer sees
raises one global banner under the header ("Sign in required" / "Not allowed"
/ "Rejected by repository policy" for `policy_violation`) with a button that
opens the popover focused on the token field; the banner clears once a sign-in
succeeds. `src/lib/useAuth.ts` holds the identity; pages call
`can(auth, repo, "admin")` to hide admin-only actions (policy save, keys
add/remove, webhook add/test/delete, rules save, repository deletion, token
management). When the identity is unknown nothing is hidden and the server
stays the authority.

`/admin/tokens` (linked from the popover as "Manage tokens" for admins) lists
`GET /auth/tokens` (name, id, bound agent, scopes, created/by, expiry, last
used) and creates tokens with a scope editor (repository or `*` + read/write/
admin, with a warning for scopes the current principal does not hold), optional
agent binding and a TTL preset or custom seconds. The plaintext token is shown
exactly once in a banner with **Copy token** and **Use in this browser**
(stores it in the settings). Delete asks for confirmation.

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
~30 commits with agents, intents and tasks (including a renamed file, a
trusted, an untrusted and an invalid signature), four pull requests with
reviews, comments and checks, leases with expiries, nine knowledge notes (two
stale, one draft from the merge of PR #1, with `valid_at`), registered keys for
`planner-1` and `reviewer-1`, three access tokens, a policy, two webhooks, tag
rules and a simulated event stream (`commit` events carry `summary`). A small
**mock data** badge appears in the header. Mutations (reviews, merges with a
generated draft note, leases, notes incl. validate/accept, policy, keys,
webhooks, rules, tokens, repository deletion) work in memory and enforce the
access rules. Mock sign-in: no token = open mode (anonymous root);
`root-secret` = root; `ci-secret` = `ci-bot` with write on `acme/shop`;
`reader-secret` = read everywhere (try it to see hidden admin actions and the
403 prompt). Blame and code search derive from the mock history; the mock's
`blame` endpoint reuses the client algorithm and returns the server shape.

## Pages

| route | what |
| --- | --- |
| `/` | repository list |
| `/r/:repo` | repo home: tree of the default branch, latest commit, README, tabs |
| `/r/:repo/tree/:rev/*` | directory browser with breadcrumbs and file finder (`t`) |
| `/r/:repo/blob/:rev/*` | file view: line numbers, syntax highlighting, raw toggle, markdown preview, Code/Blame switch, history link |
| `/r/:repo/blame/:rev/*` | blame from `GET /blame/{rev}?path=`: commit, agent/author, age and message per line group, intent on hover, age heat strip, `truncated` banner; client-side fallback for servers without the endpoint |
| `/r/:repo/commits?rev=&agent=&task=&path=` | history grouped by day with agent badges, intents, task chips, filters |
| `/r/:repo/commit/:id` | commit metadata, signature badge (unsigned / verified / valid but untrusted key / invalid with reason) with signer, key id and a link to Keys, linked knowledge notes (`notes`), file list and unified diff with rename rendering |
| `/r/:repo/compare/:base...:head` | diff between two revisions with commits ahead, per-file stats and a "Create pull request" button that prefills `/pulls/new` |
| `/r/:repo/search?q=&tab=code\|commits\|notes&rev=` | repository search (see below) |
| `/r/:repo/pulls` · `/pulls/:n` · `/pulls/new` | pull request list, detail (conversation, commits, files changed with inline comments, checks), creation form |
| `/r/:repo/agents` | agent table plus live event feed (SSE with polling fallback) |
| `/r/:repo/leases` | lease board with countdowns, renew/release, claim form |
| `/r/:repo/knowledge` · `/knowledge/:id` | notes with `draft`/`stale`/`superseded` badges, `valid at` commit, tag chips, search with BM25 `score`, path/commit/pull filters, **include drafts** (`drafts=1`), **Stale notes** view (`GET /knowledge/stale` with `changed_paths`) and Validate / Accept / Discard actions; context panel with `agents_active`, suggested tags and an **include files** toggle with token budget; note view with validity box, draft/stale banners, new note form (valid_at, draft) |
| `/admin/tokens` | server-level token admin: list, create (scopes, agent binding, TTL), one-time plaintext reveal with copy, delete |
| `/r/:repo/settings` | **Policy**: protected branches, PR/approval/check/intent/agent requirements, **require signed commits** (`require_signed`, with a pointer to Keys), allowed merge/push agents, max files per commit (`GET/PUT /policy`); read-only with a banner for non-admin tokens |
| `/r/:repo/settings/keys` | **Keys**: Ed25519 public keys trusted per agent (`GET/POST/DELETE /keys`): list grouped by agent with copy, register form (agent + public key from `agit key show`), remove |
| `/r/:repo/settings/webhooks` | **Webhooks**: list with active/signed badges, create (url, kind chips, secret), test (`POST /webhooks/{id}/test`), delete |
| `/r/:repo/settings/rules` | **Knowledge rules**: pattern → tags editor (`GET/PUT /knowledge/rules`) |
| `/r/:repo/settings/danger` | **Danger zone**: delete the repository with typed confirmation |

### Blame

The page calls `GET /repos/{repo}/blame/{rev}?path=&max_commits=500` and maps
`lines[].commit` to the `commits` table (newest first); `truncated: true`
shows a banner saying older lines are attributed to the oldest commit examined.
Only when the endpoint answers a bare `404` (older server without the route) or
in mock mode's absence of a server does it fall back to `src/lib/blame.ts`,
which walks `GET /commits?rev=&path=&limit=200`, fetches the blob at each
commit via `GET /tree/{id}?path=` (4 in flight) and attributes lines with a
Myers line diff; that path is labelled "client-side" in the header. Consecutive
lines from one commit form a group with the short id, agent badge (or author),
relative date and message; the gutter tooltip shows the intent; the left strip
fades with age.

### Signatures and keys

`CommitDetail.signature` drives a badge on the commit page: **Unsigned**
(outline), **Verified** (green: `status: valid` and `trusted: true`), **Valid,
untrusted key** (yellow: valid signature but the key is not registered for the
signer; the meta row links to Settings → Keys) and **Invalid signature** (red,
with the server's `reason`). Registering or removing a key in the Keys tab
flips the badge on the next load.

### Renames

Changes with `kind: "renamed"` render an **R** badge and `from → path` with a
`similarity%` pill in file lists (commit, compare, PR files) and in the diff
header; a rename without content changes says so instead of showing `+0 −0`.
Patches that name different `a/` and `b/` paths are matched to the change by
either side. The server's browsing endpoints do not run rename detection yet
(only the CLI does), so against a live server a move still shows as D + A; the
mock has a renamed fixture (`docs/ARCHITECTURE.md → docs/adr/0001-architecture.md`, 67 %).

### Knowledge v2

Lists and search results carry `draft` (purple), `stale` (yellow) and
`superseded` badges plus the `valid at` commit; `q=` results show the BM25
`score`. The **Stale notes** view calls `GET /knowledge/stale` and lists the
`changed_paths`; **Validate at HEAD** posts `/validate`. With **include
drafts** checked (`drafts=1`) draft rows get **Accept** (`POST /accept`) and
**Discard** (`DELETE`). The note page shows a validity box (`valid_at` linked
to the commit, fresh/stale, compare-with-HEAD link), draft/stale banners with
the same actions, and the commit page lists `CommitDetail.notes`. The context
panel shows `agents_active`, suggested tags, fresh notes before stale ones and,
with **include files**, the `files` returned for `include=files&budget=` with a
`truncated` badge and collapsible text.

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
a one-line summary and a link target. `commit` events (one per new commit on a
pushed branch) show `<short id> on <branch> · <summary> → <intent>` and open the
commit; the activity feed renders the same fields plus the task. Unread state lives in
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
      client.ts         typed fetch wrapper, error envelope -> ApiError, 401/403 listeners,
                        SSE/polling stream, live/mock facade (`api`), `initApi()` health probe
      mock.ts           in-memory implementation of the same `Api` interface with fixtures
      settings.ts       localStorage settings (base URL, token, agent)
    lib/
      useAuth.ts        whoami store: `useAuth()`, `can()`, `hasAccess()`, `refreshAuth()`
      blame.ts          client-side blame fallback: history walk + Myers line diff, caches
      search.ts         recursive tree listing + blob grep with progress/cancel, caches
      useDismiss.ts     close popovers on outside click / Escape
      diff.ts           unified diff parser for the `patch` strings
      highlight.ts      lazy highlight.js, per-line HTML splitting
      markdown.ts       lazy marked + DOMPurify
      format.ts         time ago, short ids, byte sizes, language detection
      useAsync.ts       tiny data hooks (useAsync, useMutation, useNow)
    components/
      Layout.tsx        header (logo, breadcrumb, search box, mock badge, bell, principal chip,
                        settings/sign-in popover), global auth prompt, footer, toasts
      SearchBox.tsx     header search input with the `/` shortcut
      Notifications.tsx bell, dropdown, unread state, `describeEvent()` summaries
      RepoShell.tsx     repo tabs (Code, Commits, Pull requests, Agents, Leases, Knowledge, Settings) + context
      TreeBrowser.tsx   tree table, breadcrumbs, latest commit bar, README, file finder
      BranchSelector.tsx, CommitList.tsx, CodeView.tsx, DiffView.tsx, Markdown.tsx,
      Toasts.tsx, ui.tsx (badges incl. SignatureBadge, NoteBadges, ChangePath, Score,
                        CopyButton; skeletons, empty/error states)
    pages/              one file per route (AdminTokens.tsx is the server-level page)
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
