# RFC 0003: Knowledge base

- Status: **Partially implemented** (server-side, see below)
- Phase: 4
- Depends on: RFC 0001 (object model)

## Implementation status

What exists today lives in `agit-server` (`crates/agit-server/src/knowledge.rs`) and is specified in `docs/API.md`, section "Knowledge notes":

- Notes are **mutable JSON files** under `<repo>/knowledge/notes/`, not `0x04` note objects on `refs/knowledge/<branch>`. They do not clone, branch or merge with the code yet. Tag rules are `knowledge/rules.json` (`[{pattern, tags}]`), not `rules.toml`.
- Typed tags (`namespace:value`) are validated; the reserved namespaces are a convention, not enforced. Links are the fields `paths`, `commits` and `pulls`; `symbol`, `note` and `url` links do not exist. Supersession is the mutable field `superseded_by`.
- `valid_from` exists as `valid_at` (default HEAD at creation). The server computes `stale` on read by diffing the trees of `valid_at` and HEAD against the note's `paths` (the "confidence decay" open question is answered with a boolean instead of a decay). `GET /knowledge/stale` lists stale notes with the changed paths; `POST /knowledge/notes/{id}/validate` moves `valid_at` forward.
- Full-text search is an in-memory per-repository BM25 index (no tantivy), built lazily and maintained on writes. Queries combine `q` with `tags`, `path`, `commit` and `pull` filters.
- Draft notes are generated from merged pull requests (`draft: true`, `note.draft` event) and must be accepted or deleted.
- Context packs: `GET /context?path=&include=files&budget=` returns notes (fresh first), recent commits, leases, open PRs, active agents and budgeted file contents; `agit know context` is the CLI form.
- Commit detail (`GET /commits/{rev}`) lists notes linking the commit or covering its changed paths.

Not implemented: note objects in the object store, the knowledge tree and `refs/knowledge/*`, versioned note history, cross-branch knowledge, `symbol` links, and the `agit know get`/`--since`/`--until` CLI forms. The design below is unchanged and remains the target.

## Summary

Domain knowledge about a codebase (decisions, gotchas, specs, how-tos) is stored in the repository as versioned `note` objects with typed tags and links to paths, symbols, commits and PRs. Tag rules map path patterns to tags so an agent knows which notes to read before changing a path and which tags to attach after. A query API returns compact, token-efficient results.

## Motivation

The knowledge that explains a codebase lives in wikis, chat threads, PR descriptions and people's heads. The agent editing the code sees none of it, so it repeats known mistakes and re-derives known decisions. Putting notes next to the code, versioned with it, tagged in a way that maps to paths, and queryable in one call fixes the delivery problem: the right notes arrive at the moment of the change.

## Design

### Note object

A fourth object kind, tag `0x04`, canonical JSON:

| field | type | notes |
| --- | --- | --- |
| `title` | string | one line |
| `body` | string | markdown |
| `tags` | string[] | typed, `namespace:value`, sorted, unique |
| `links` | `{type, value}[]` | `type` in `path`, `symbol`, `commit`, `pr`, `note`, `url` |
| `valid_from` | id? | commit at which the note became true |
| `superseded_by` | id? | newer note that replaces this one |
| `author` | `{name, email}` | |
| `agent` | agent info? | same shape as in commits |
| `confidence` | number | 0.0 to 1.0 |
| `time_ms` | i64 | |

Notes are immutable like every object, so an existing note object can never gain a `superseded_by` value after the fact. Editing therefore writes a *new* note object that links back to the old one (`{type: "note", value: <old id>}`), and the knowledge tree (below) maps the stable note name to the new object. A reader that holds an old id learns it is superseded by asking the tree, not the object. Whether `superseded_by` should remain a field on the object at all, or live only in the tree, is listed under open questions.

### Typed tags

Reserved namespaces: `area` (module or domain), `kind` (`decision`, `gotcha`, `spec`, `howto`, `rule`), `lang`, `service`. Any other namespace is allowed and indexed. A tag without a namespace is rejected.

### Tag rules

A TOML file, `rules.toml`, in the knowledge tree:

```toml
[[rule]]
paths = ["src/billing/**", "migrations/billing_*"]
tags  = ["area:billing", "service:billing"]

[[rule]]
paths = ["**/*.rs"]
tags  = ["lang:rust"]
```

Rules answer: "which tags describe this path" (union over matching rules). From that, "which notes should I read before touching this path" is "notes whose tags intersect the path's tags, ranked", and "which tags should a note about this change carry" is the same set.

### Storage

Notes and `rules.toml` form a **knowledge tree** committed on `refs/knowledge/<branch>`, parallel to `refs/heads/<branch>`:

```
notes/<note-name>        -> note object (mode: file, special kind)
rules.toml
```

A knowledge commit is an ordinary commit object whose tree points at note objects. Because it is a tree, knowledge merges with the same three-way machinery as code, and the history of a note is `agit log refs/knowledge/main -- notes/<name>`. Branching code branches knowledge; merging code merges knowledge. The server indexes the knowledge tree of every branch head for queries.

### Query API

```sh
agit know search --path src/billing/invoice.rs
agit know search --tag area:billing --tag kind:gotcha
agit know search --since <commit> --until <commit>
agit know search --text "rotation"
```

Results are ranked by tag overlap, confidence, recency and validity, and return `id, title, tags, confidence, valid_from, summary` (first line of the body). `--full` or `agit know show <id>` returns bodies. `--limit` defaults to 10. All output is JSON with a `version` field.

### CLI

- `agit know add --title … --tag … --link path:src/x.rs [--body-file f.md]`: writes a note, commits it on the current knowledge branch.
- `agit know link <note> <type>:<value>`: appends a link by writing a new note version.
- `agit know tags [--path p]`: lists tags, or the tags that rules assign to a path.
- `agit know search …` as above.

### Context packs

`agit context <path|task>` (RFC-less for now, Phase 4) calls the knowledge query as one of its inputs and returns notes alongside files, history, leases and PRs.

## Alternatives

- **Notes as markdown files in the code tree (`docs/notes/*.md` with front matter).** Attractive for simplicity; rejected as the primary format because front matter is unstructured, links are not validated, and tooling cannot distinguish a note from documentation. Importing such files into notes is a planned convenience.
- **Notes in an external database only.** Rejected: knowledge must clone, branch and merge with the code it describes.
- **Free-form tags.** Rejected: typed tags make rules and ranking tractable and keep `area:auth` and `kind:auth` apart.
- **Mutable notes.** Rejected: immutability is what makes `valid_from` and provenance trustworthy.

## Open questions

- Supersession mechanics: whether `superseded_by` lives in the new note (pointing back) only, with the tree deciding "current", or whether the tree entry carries a small metadata record. The former keeps objects immutable without a second format.
- Whether `symbol` links need a language-server-grade resolver or a convention (`<lang>:<fully qualified name>`).
- Server-side index technology for full text (tantivy is the current candidate).
- How confidence decays, if at all, when the linked paths change substantially after `valid_from`.
- Whether knowledge should be allowed on a branch that has no code branch of the same name (shared knowledge across all branches).
