# agit-knowledge (planned)

Status: 📋 planned (Roadmap Phase 4). This directory holds only this README;
there is no crate here yet and it is not a workspace member.

`agit-knowledge` will implement the knowledge base: domain knowledge stored as
versioned objects next to the code, so an agent can ask "what do I need to
know before touching `src/billing/`?" and get a compact, ranked answer.

Planned scope:

- `note` objects: title, markdown body, typed tags (`area:auth`,
  `kind:decision`, `kind:gotcha`, `kind:spec`, `lang:rust`,
  `service:billing`), links to paths, symbols, commits and PRs, validity
  (`valid_from` commit, `superseded_by`), author or agent, confidence.
- Tag rules mapping path patterns to tags.
- Query API: by path, tag set, commit range, full text; summaries first,
  bodies on demand.
- CLI: `agit know add | query | link | tags`.
- Server-side index for fast queries; local fallback that scans the
  knowledge tree.

Design: `docs/rfcs/0003-knowledge-base.md`.

This component is designed to be split into its own repository and linked
back as a submodule (`.agitmodules`).
