# agit-git-bridge (planned)

Status: 📋 planned (Roadmap Phase 3). This directory holds only this README;
there is no crate here yet and it is not a workspace member.

`agit-git-bridge` will let teams adopt agit incrementally by moving history in
both directions between git and agit repositories:

- `agit git import <url>`: convert a git repository (SHA-1 or SHA-256) into
  agit objects, preserving authorship, timestamps, and messages. Trailers such
  as `Intent:` and `Agent:` are lifted into the structured commit fields.
- `agit git export`: write an agit history as a git repository.
- Bidirectional sync with a persistent SHA mapping table (git object id to
  agit object id and back) so repeated syncs are stable and idempotent.
- Lossless round-trip for everything git can represent; agit-only metadata
  (`intent`, `task`, `agent`, `meta`, checkpoints) is carried as commit
  trailers on the git side.

This component is designed to be split into its own repository and linked
back as a submodule (`.agitmodules`).
