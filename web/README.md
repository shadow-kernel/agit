# agit web UI (planned)

Status: 📋 planned (Roadmap Phase 3). This directory holds only this README.

The web UI is the human-facing side of the agit platform: clean, GitHub-like,
and fast, with the agent-native views GitHub does not have.

Planned features, in delivery order:

1. Code browsing with syntax highlighting and symbol navigation; history and
   blame; diffs.
2. Pull requests and structured reviews (`docs/rfcs/0005-pull-requests-and-reviews.md`).
3. Branches and workspaces.
4. Live agent activity fed by the server event stream.
5. Lease board: who is working where, with expiry.
6. Knowledge browser (`docs/rfcs/0003-knowledge-base.md`).

Stack: TypeScript, React, server-side rendering for first-paint speed. The UI
reads and writes strictly through the same public API that agents use. There
are no private endpoints.

Design: `docs/rfcs/0006-web-ui.md`.

This component is designed to be split into its own repository and linked
back as a submodule (`.agitmodules`).
