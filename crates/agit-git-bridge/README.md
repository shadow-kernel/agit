# agit-git-bridge

Import git history into agit. Uses the `git` command line (`rev-list`,
`cat-file --batch`, `for-each-ref`), so it works wherever git is installed and
needs no libgit2.

```sh
agit git import /path/to/git-repo --into ./repo        # new agit repository
agit git import . --branch main --no-tags              # into the current agit repository
```

What is preserved: full commit graph (merges included), author and timestamp,
messages, branches, lightweight and annotated tags (peeled), file modes,
symlinks, submodule pins. Trailers `Intent:`, `Task:`, `Agent:`, `Model:` and
`Run-Id:` become the structured commit fields; other trailers land in
`meta["git.trailer.<key>"]`; the original SHA is kept in `meta["git.sha"]`.
A mapping table `.agit/git-map.json` makes re-imports incremental and
idempotent.

Planned: export back to git and continuous two-way sync (RFC in
`docs/ROADMAP.md`, Phase 3).
