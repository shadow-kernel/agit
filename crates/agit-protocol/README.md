# agit-protocol (planned)

Status: 📋 planned (Roadmap Phase 2). This directory holds only this README;
there is no crate here yet and it is not a workspace member.

`agit-protocol` will define the wire protocol shared by the CLI, the server,
the SDKs, and the git bridge:

- Message schemas for refs negotiation, object want/have exchange, and
  streaming pack transfer, carried over HTTP/2 and gRPC.
- Authentication: scoped, short-lived agent tokens; token exchange for humans.
- Idempotency keys on every mutating request so a crashed or retried agent
  cannot double-apply a push.
- Versioned schemas with a `version` field; see `spec/` for the format spec
  once it exists.

Design notes live in `docs/ARCHITECTURE.md` (section "Planned server
architecture") and `docs/rfcs/0004-throughput.md`.

This component is designed to be split into its own repository and linked
back as a submodule (`.agitmodules`).
